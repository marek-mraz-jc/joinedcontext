import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { axeViolations } from "./axe";

// T-6096: the Flows page is gone. Its ingestion set-ups start from Data sources, and the old
// address lands on the project's home. `vite preview` has no Portal API behind it, so the browser
// answers it; the clicks, the routing and the rendering are the real ones.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS_DIR;

const FEED = {
  type: "object",
  required: ["name", "url"],
  properties: {
    name: { type: "string", title: "Flow name", description: "Lowercase, for example citybikes.", default: "citybikes" },
    url: { type: "string", title: "Feed URL", description: "For example https://gbfs.example.org/station_status.json", format: "uri" },
  },
};
const MQTT = {
  type: "object",
  required: ["name", "brokerUrl", "topic"],
  properties: {
    name: { type: "string", title: "Flow name", description: "Lowercase, for example sensors.", default: "sensors" },
    brokerUrl: { type: "string", title: "Broker URL", description: "For example mqtts://broker.example.org:8883", format: "uri" },
    topic: { type: "string", title: "Topic", description: "For example city/sensors/+/air" },
  },
};

const blueprint = (name: string, title: string, description: string, riskClass: string, category = "ingestion", parameterSchema: unknown = FEED) => ({
  apiVersion: API,
  kind: "Blueprint",
  metadata: { name, namespace: "org", title: { en: title }, description: { en: description } },
  spec: {
    version: "1.0.0",
    category,
    riskClass,
    allowedRoles: ["portal-approver"],
    parameterSchema,
  },
});

const BLUEPRINTS = [
  blueprint("gbfs-stations", "City-bike stations from a GBFS feed", "Reads the stations' free bikes and docks from a GBFS feed and writes them into a Context Space.", "yellow"),
  blueprint("http-json-feed", "Poll a JSON feed into a Context Space", "Reads a JSON document on a schedule and writes one entity per record.", "yellow"),
  blueprint("mqtt-feed", "Subscribe to an MQTT feed", "Reads the messages of an MQTT topic and writes them into a Context Space.", "red", "ingestion", MQTT),
  blueprint("app-from-prompt", "An application from a prompt", "Generates an App from a description.", "yellow", "application"),
];

async function stubApi(page: Page, { blueprints = BLUEPRINTS, refuse = false } = {}): Promise<string[]> {
  const posted: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-approver"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (url.pathname === "/api/v1/blueprints") return json({ apiVersion: API, kind: "List", items: blueprints });
    if (request.method() === "POST" && url.pathname.endsWith(`/projects/${PROJECT}/flows`)) {
      posted.push(request.postData() ?? "");
      return refuse
        ? json({ type: "about:blank", title: "Unprocessable", status: 422, detail: "The parameters break the blueprint's schema.", errors: ["brokerUrl: must be an mqtts:// address"] }, 422)
        : json({ apiVersion: API, kind: "Change", metadata: { name: "chg-60960001", namespace: PROJECT }, status: { lane: "yellow", phase: "PendingApproval", plan: { create: 3 } } }, 202);
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return posted;
}

async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/${state}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("ready-made set-ups on Data sources", () => {
  test("the old Flows address opens the home and the sidebar has no Flows", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/flows?lang=en`);
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/home`));
    const nav = page.getByRole("navigation", { name: "Main navigation" });
    await expect(nav.getByRole("link", { name: "Data sources" })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Flows", exact: true })).toHaveCount(0);
  });

  test("a steward sets up a GBFS feed from Data sources and the change is proposed", async ({ page }) => {
    const posted = await stubApi(page);
    await page.goto(`/projects/${PROJECT}/home?lang=en`);
    await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "Data sources" }).click();

    const section = page.getByRole("region", { name: "Ready-made set-ups" });
    await expect(section.getByRole("listitem")).toHaveCount(3);
    await expect(section.getByText("An application from a prompt")).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, "datasources-setups");

    await section.getByRole("listitem").filter({ hasText: "City-bike stations" }).getByRole("button", { name: "Set up" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Set up: City-bike stations from a GBFS feed" })).toBeVisible();
    await page.getByLabel(/Feed URL/).fill("https://gbfs.example.org/station_status.json");
    await shot(page, "wizard-filled");
    await page.getByRole("button", { name: "Propose change" }).click();
    await expect(page.getByText(/chg-60960001|approv/i).first()).toBeVisible();
    expect(JSON.parse(posted[0])).toMatchObject({ blueprint: "gbfs-stations", version: "1.0.0", parameters: { url: "https://gbfs.example.org/station_status.json" } });
    // Proposed once: the form is closed against a second Change.
    await expect(page.getByLabel(/Feed URL/)).toBeDisabled();
    await expect(page.getByRole("button", { name: "Propose change" })).toBeDisabled();
    await shot(page, "wizard-proposed");

    await page.getByRole("button", { name: "Back to data sources" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Data sources" })).toBeVisible();
  });

  test("a refused set-up names every rule it broke", async ({ page }) => {
    await stubApi(page, { refuse: true });
    await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
    await page.getByRole("region", { name: "Ready-made set-ups" }).getByRole("listitem").filter({ hasText: "MQTT" }).getByRole("button", { name: "Set up" }).click();
    await page.getByLabel(/Broker URL/).fill("mqtt://broker.example.org:1883");
    await page.getByLabel(/Topic/).fill("city/sensors/+/air");
    await page.getByRole("button", { name: "Propose change" }).click();
    const alert = page.getByRole("alert").filter({ hasText: "The parameters break" });
    await expect(alert).toContainText("brokerUrl: must be an mqtts:// address");
    await shot(page, "wizard-refused");
  });

  // T-6825: the type picker offers only inputs the pipeline runner (ekiben) builds and the
  // platform accepts; the server refuses any other one on save with a 422 naming spec.type.
  test("the type picker offers only the inputs the runner builds", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
    await page.getByRole("button", { name: "New data source" }).first().click();
    const type = page.getByTestId("form-page").getByLabel("Type", { exact: true });
    await expect(type.locator('option[value="nats"]')).toHaveCount(1);
    const offered = await type.locator("option").evaluateAll((options) => options.map((o) => (o as HTMLOptionElement).value));
    // sql_select and aws_s3 joined once ekiben built them (T-7054, T-7079); ftp and imap are
    // ekiben's own (T-7092).
    for (const name of ["kafka", "amqp_0_9", "stdin", "http_server"]) expect(offered).not.toContain(name);
    expect(offered).toEqual(expect.arrayContaining(["http_client", "nats", "nats_jetstream", "csv", "websocket", "sql_select", "aws_s3", "ftp", "imap"]));
    await type.selectOption("nats");
    await shot(page, "runner-input-nats");
  });

  // T-7092: ekiben's ftp input from the type picker. Every field says what it takes with an
  // example, tls is a dropdown whose none says it needs the operator's quota, the password is a
  // secret reference (name and key, never a value), and the proposed DataSource carries it as a
  // ${VAR} of spec.secrets.
  test("an ftp source is set up from its form, its password a secret reference", async ({ page }) => {
    await stubApi(page);
    const posted: string[] = [];
    await page.route(`**/api/v1/projects/${PROJECT}/datasources**`, async (route) => {
      const request = route.request();
      if (request.method() !== "POST") return route.fallback();
      posted.push(request.postData() ?? "");
      const dry = new URL(request.url()).searchParams.has("dryRun");
      return route.fulfill({
        status: dry ? 200 : 202,
        contentType: "application/json",
        body: JSON.stringify(
          dry
            ? {
                apiVersion: API,
                kind: "DataSource",
                metadata: { name: "exports", namespace: PROJECT },
                plan: { fields: [{ path: "spec.type", to: "ftp" }, { path: "spec.input.address", to: "files.example.org:21" }, { path: "spec.input.path", to: "/exports/*.csv" }] },
                verdict: { ok: true, findings: [], checkedAt: "2026-11-15T21:30:00Z", inputDigest: "sha256:7092" },
              }
            : { apiVersion: API, kind: "Change", metadata: { name: "chg-70920001", namespace: PROJECT }, status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } } },
        ),
      });
    });
    await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
    await page.getByRole("button", { name: "New data source" }).first().click();
    const form = page.getByTestId("form-page");
    await form.getByLabel("Type", { exact: true }).selectOption("ftp");
    await expect(form.getByText("The FTP server as host or host:port, for example files.example.org:21", { exact: false })).toBeVisible();
    // tls sits in the form's folded TLS section, as every input's TLS settings do.
    await form.getByText("TLS certificates", { exact: true }).first().click();
    await expect(form.getByText("allow_plain_ftp quota", { exact: false })).toBeVisible();
    const tls = form.getByLabel("Tls", { exact: true });
    await expect(tls.locator("option:checked")).toHaveText(/explicit/);
    await expect(tls.locator("option")).toHaveText([/explicit/, /none/]);
    await shot(page, "ftp-empty");
    await page.getByRole("button", { name: "Check" }).click();
    await expect(form.getByText(/required/i).first()).toBeVisible();
    expect(posted).toEqual([]);
    await shot(page, "ftp-refused");
    await form.getByRole("textbox", { name: "Name", exact: true }).fill("exports");
    await form.getByRole("textbox", { name: "Address", exact: true }).fill("files.example.org:21");
    await form.getByRole("textbox", { name: "Path", exact: true }).fill("/exports/*.csv");
    await form.getByRole("textbox", { name: "Username", exact: true }).fill("exports");
    await form.getByPlaceholder("Secret name").fill("ftp-login");
    await form.getByPlaceholder("Key").fill("password");
    await shot(page, "ftp-filled");
    await page.getByRole("button", { name: "Check" }).click();
    await expect.poll(() => posted.length).toBe(1);
    const body = JSON.parse(posted[0]) as { spec: { type: string; input: Record<string, unknown>; secrets: { name: string; key: string; envVar: string }[] } };
    expect(body.spec.type).toBe("ftp");
    expect(body.spec.input).toMatchObject({ address: "files.example.org:21", path: "/exports/*.csv", username: "exports", tls: "explicit" });
    expect(body.spec.input.password).toMatch(/^\$\{[A-Z][A-Z0-9_]*\}$/);
    const envVar = String(body.spec.input.password).slice(2, -1);
    expect(body.spec.secrets).toEqual([{ name: "ftp-login", key: "password", envVar }]);
    await shot(page, "ftp-checked");
    expect(await axeViolations(page)).toEqual([]);
  });

  test("a person who may run no set-up sees Data sources without the section", async ({ page }) => {
    await stubApi(page, { blueprints: [] });
    await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: "Data sources" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Ready-made set-ups" })).toHaveCount(0);
    await shot(page, "datasources-none");
  });

  // T-6572: a JSON feed that keeps its records in an array (GBFS data.stations) names that array
  // on the template, so the source sends one station per message.
  test("a JSON template names the array its records sit in, and refuses a path that is not names", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/datasources/new?lang=en`);
    await page.getByText("Start from a template").click();
    await page.getByRole("button", { name: /REST API \(JSON\)/ }).click();
    await page.getByLabel("Address of the API").fill("https://gbfs.example.org/gbfs/2.2/helsinki/en/station_information.json");
    const records = page.getByTestId("datasource-templates").getByRole("textbox", { name: "Records are in" });
    await expect(records).toBeVisible();
    await shot(page, "records-empty");
    await records.fill("data stations");
    await page.getByRole("button", { name: "Fill the form and check" }).click();
    await expect(page.getByText("Give the path as names joined by dots, for example data.stations.")).toBeVisible();
    await shot(page, "records-refused");
    await records.fill("data.stations");
    await expect(page.getByText("Give the path as names joined by dots, for example data.stations.")).toBeHidden();
    await shot(page, "records-filled");
    await page.getByRole("button", { name: "Fill the form and check" }).click();
    // The template fills the form that is saved: the connection's own field holds the path.
    const form = page.getByRole("group", { name: "HTTP endpoint" });
    await expect(form.getByRole("textbox", { name: "Records are in" })).toHaveValue("data.stations");
    await expect(form.getByRole("textbox", { name: /URL/ })).toHaveValue("https://gbfs.example.org/gbfs/2.2/helsinki/en/station_information.json");
    await shot(page, "records-applied");
    expect(await axeViolations(page)).toEqual([]);
  });

  // T-7086: a statistics office's workbook is read one record per row of the sheet its tab names;
  // a name no workbook can hold is refused before anything is proposed, and the form that is
  // saved carries the format and the sheet.
  test("a spreadsheet template names its sheet, refuses a name no workbook holds, and fills the form", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/datasources/new?lang=en`);
    await page.getByText("Start from a template").click();
    await page.getByRole("button", { name: /Spreadsheet \(Excel, ODS\)/ }).click();
    const gallery = page.getByTestId("datasource-templates");
    const sheet = gallery.getByRole("textbox", { name: "Sheet", exact: true });
    await expect(sheet).toBeVisible();
    await expect(gallery.getByText("The sheet to read, by the name on its tab, for example Stations 2026", { exact: false })).toBeVisible();
    await shot(page, "sheet-empty");
    await page.getByLabel("Address of the spreadsheet").fill("https://data.example.org/stats/stations-2026.xlsx");
    await sheet.fill("2026/Q1");
    await page.getByRole("button", { name: "Fill the form and check" }).click();
    const refused = "A sheet name is 1 to 31 characters with none of : \\ / ? * [ ].";
    await expect(gallery.getByText(refused)).toBeVisible();
    await shot(page, "sheet-refused");
    await sheet.fill("Stations 2026");
    await expect(gallery.getByText(refused)).toBeHidden();
    await shot(page, "sheet-filled");
    await page.getByRole("button", { name: "Fill the form and check" }).click();
    const form = page.getByRole("group", { name: "HTTP endpoint" });
    await expect(form.getByRole("textbox", { name: "Sheet", exact: true })).toHaveValue("Stations 2026");
    await expect(form.getByRole("combobox", { name: "Body format" })).toHaveValue(/xlsx|1/);
    await expect(form.getByRole("textbox", { name: /URL/ })).toHaveValue("https://data.example.org/stats/stations-2026.xlsx");
    await shot(page, "sheet-applied");
    expect(await axeViolations(page)).toEqual([]);
  });

  // T-7089: a city's map layer published as a zipped shapefile is read one GeoJSON feature per
  // record; the template asks only the address, says the .prj picks the coordinate system, and
  // the form it fills carries format shapefile.
  test("a shapefile template asks for the zip's address and fills the form with format shapefile", async ({ page }) => {
    await stubApi(page);
    await page.route(`**/api/v1/projects/${PROJECT}/datasources**`, async (route) =>
      route.request().method() !== "POST"
        ? route.fallback()
        : route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({
              apiVersion: API,
              kind: "DataSource",
              metadata: { name: "kaupunginosat", namespace: PROJECT },
              plan: { fields: [{ path: "spec.http.url", to: "https://data.example.org/kartta/kaupunginosat.zip" }, { path: "spec.http.format", to: "shapefile" }] },
              verdict: { ok: true, findings: [], checkedAt: "2026-11-15T21:30:00Z", inputDigest: "sha256:7092" },
            }),
          }),
    );
    await page.goto(`/projects/${PROJECT}/datasources/new?lang=en`);
    await page.getByText("Start from a template").click();
    await page.getByRole("button", { name: /Shapefile \(zipped\)/ }).click();
    const gallery = page.getByTestId("datasource-templates");
    await expect(gallery.getByText("The .prj names the coordinate system", { exact: false })).toBeVisible();
    await expect(gallery.getByRole("textbox", { name: "Sheet", exact: true })).toHaveCount(0);
    await shot(page, "shapefile-empty");
    await page.getByLabel("Address of the zip").fill("not an address");
    await page.getByRole("button", { name: "Fill the form and check" }).click();
    // The address is what is wrong, so the address says so; the name it derives is not blamed.
    await expect(gallery.getByText("Give a full address starting with https:// or http://.")).toBeVisible();
    await expect(gallery.getByText("Give a name of lowercase letters, digits and hyphens, at most 63 characters.")).toHaveCount(0);
    await shot(page, "shapefile-refused");
    await page.getByLabel("Address of the zip").fill("https://data.example.org/kartta/kaupunginosat.zip");
    await expect(gallery.getByText("Give a full address starting with https:// or http://.")).toHaveCount(0);
    await shot(page, "shapefile-filled");
    await page.getByRole("button", { name: "Fill the form and check" }).click();
    const form = page.getByRole("group", { name: "HTTP endpoint" });
    await expect(form.getByRole("combobox", { name: "Body format" })).toHaveValue(/shapefile|2/);
    await expect(form.getByRole("textbox", { name: /URL/ })).toHaveValue("https://data.example.org/kartta/kaupunginosat.zip");
    await shot(page, "shapefile-applied");
    expect(await axeViolations(page)).toEqual([]);
  });
});
