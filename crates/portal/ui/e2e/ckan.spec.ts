import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-2849: the open-data page in a real browser, the API answered in the browser. What it shows of
// `/ckan/status` (the catalogues, what each endpoint became in them, a publication whose catalogue
// is gone), how it proposes a catalogue (by the name of a secret, never a token), what a viewer
// meets, and what it says when the status cannot be read.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "eva.steward",
  name: "Eva Steward",
  email: "eva@hel.fi",
  roles: ["portal-editor"],
};

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [
    {
      role: verbs.includes("propose") ? "steward" : "viewer",
      binding: "helsinki-people",
      scope: "project:helsinki",
      rule: { kinds: ["CkanInstance", "Endpoint"], verbs },
    },
  ],
});

const STATUS = {
  instances: [
    {
      name: "hel-fi",
      url: "https://opendata.dev.joinedcontext.com",
      organizationDefault: "helsinki",
      apiTokenRef: "ckan-api-token",
    },
  ],
  publications: [
    {
      endpoint: "public-air",
      instance: "hel-fi",
      instanceMissing: false,
      dataset: "helsinki-air-quality",
      datasetUrl: "https://opendata.dev.joinedcontext.com/dataset/helsinki-air-quality",
      resources: [
        { name: "CSV", format: "CSV", url: "https://portal.example/api/endpoint/k7m2/file.csv" },
        { name: "GeoJSON", format: "GeoJSON", url: "https://portal.example/api/endpoint/k7m2/geo.json" },
      ],
    },
    {
      endpoint: "old-traffic",
      instance: "retired-ckan",
      instanceMissing: true,
      dataset: "helsinki-traffic",
      resources: [],
    },
  ],
};

interface Stub {
  status?: { code: number; body: unknown };
  verbs?: string[];
  /** The caller administers the organization: approves roles and bindings (PF-107). */
  admin?: boolean;
}

const ADMIN_GRANTS = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      scope: "organization",
      rule: { kinds: ["Role", "RoleBinding", "Group", "CkanInstance", "Endpoint"], verbs: ["read", "propose", "approve", "delete"] },
    },
  ],
};

async function stubApi(page: Page, stub: Stub = {}): Promise<{ writes: { path: string; body: unknown }[] }> {
  const writes: { path: string; body: unknown }[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: status >= 400 ? "application/problem+json" : "application/json",
        body: JSON.stringify(body),
      });

    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (request.method() !== "GET") {
      writes.push({ path: url.pathname + url.search, body: request.postDataJSON() as unknown });
      if (url.searchParams.get("dryRun") === "All") return json({ valid: true, verdict: { ok: true, findings: [] } });
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-ckan-1", namespace: "helsinki" },
          spec: { lane: "Yellow" },
          status: { phase: "Proposed" },
        },
        202,
      );
    }
    if (stub.admin && url.pathname === "/api/v1/projects/org/permissions/me") return json(ADMIN_GRANTS);
    if (url.pathname.endsWith("/permissions/me")) return json(grants(stub.verbs ?? ["read", "propose"]));
    if (url.pathname === "/api/v1/projects/org/groups") {
      return json(LIST([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Group", metadata: { name: "ckan-editors", namespace: "org" }, spec: { members: [] } }]));
    }
    if (url.pathname === "/api/v1/projects") return json(LIST([{ name: "helsinki" }]));
    if (url.pathname === "/api/v1/projects/helsinki/ckan/status") {
      return json(stub.status?.body ?? STATUS, stub.status?.code ?? 200);
    }
    return json(LIST([]));
  });
  return { writes };
}

test.describe("the open-data page", () => {
  test("shows each catalogue and what every endpoint became in it, and a publication whose catalogue is gone", async ({
    page,
  }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/ckan?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Open-data catalogue" })).toBeVisible();

    const catalogues = page.getByRole("table", { name: "Catalogues" });
    const row = catalogues.getByRole("row", { name: /hel-fi/ });
    await expect(row.getByRole("link", { name: "https://opendata.dev.joinedcontext.com" })).toBeVisible();
    // The name of the secret the operator loaded, never a token (EP-67).
    await expect(row.getByText("ckan-api-token")).toBeVisible();

    const published = page.getByRole("region", { name: "Published endpoints" });
    await expect(published.getByText("published to hel-fi")).toBeVisible();
    await expect(published.getByRole("link", { name: "helsinki-air-quality" })).toHaveAttribute(
      "href",
      "https://opendata.dev.joinedcontext.com/dataset/helsinki-air-quality",
    );
    await expect(published.getByRole("link", { name: "CSV" })).toBeVisible();
    await expect(published.getByRole("link", { name: "GeoJSON" })).toBeVisible();
    await expect(published.getByText("catalogue retired-ckan is missing")).toBeVisible();
  });

  test("proposes a catalogue by the name of its token's secret, checked first, and clears the form once proposed", async ({
    page,
  }) => {
    const { writes } = await stubApi(page);
    await page.goto("/projects/helsinki/ckan?lang=en");
    await page.getByRole("textbox", { name: "Name", exact: true }).fill("open-data");
    await page.getByRole("textbox", { name: "URL", exact: true }).fill("https://opendata.example.org");
    await page.getByRole("combobox", { name: "Default organization", exact: true }).fill("city-office");
    await page.getByRole("option", { name: /city-office/ }).click();
    await page.getByRole("textbox", { name: "API token secret", exact: true }).fill("ckan-open-data-token");
    await page.getByRole("button", { name: "Propose catalogue" }).click();

    await expect(page.getByRole("textbox", { name: "Name", exact: true })).toHaveValue("");
    expect(writes.map((write) => write.path)).toEqual([
      "/api/v1/projects/helsinki/ckaninstances?dryRun=All",
      "/api/v1/projects/helsinki/ckaninstances",
    ]);
    expect(writes[1].body).toMatchObject({
      kind: "CkanInstance",
      metadata: { name: "open-data", namespace: "helsinki" },
      spec: {
        url: "https://opendata.example.org",
        organizationDefault: "city-office",
        apiTokenRef: { name: "ckan-open-data-token", key: "apiToken" },
      },
    });
  });

  test("refuses an address that is not http(s) before anything is proposed, and keeps what was typed", async ({
    page,
  }) => {
    const { writes } = await stubApi(page);
    await page.goto("/projects/helsinki/ckan?lang=en");
    await page.getByRole("textbox", { name: "Name", exact: true }).fill("open-data");
    await page.getByRole("textbox", { name: "URL", exact: true }).fill("javascript:alert(1)");
    await page.getByRole("textbox", { name: "API token secret", exact: true }).fill("ckan-open-data-token");
    await page.getByRole("button", { name: "Propose catalogue" }).click();

    await expect(page.getByText("The address starts with http:// or https://.")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "URL", exact: true })).toBeFocused();
    await expect(page.getByRole("textbox", { name: "Name", exact: true })).toHaveValue("open-data");
    expect(writes).toEqual([]);
  });

  test("a viewer reads why there is no catalogue form, and still sees the catalogues (T-4997)", async ({ page }) => {
    const { writes } = await stubApi(page, { verbs: ["read"] });
    await page.goto("/projects/helsinki/ckan?lang=en");
    await expect(
      page.getByText("Your role lets you look at these, not create one here.", { exact: false }),
    ).toBeVisible();
    await expect(page.getByRole("table", { name: "Catalogues" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Propose catalogue" })).toHaveCount(0);
    await expect(page.getByRole("textbox", { name: "API token secret", exact: true })).toHaveCount(0);
    expect(writes).toEqual([]);
  });

  test("says the status could not be read, and never that the project has no catalogue", async ({ page }) => {
    await stubApi(page, {
      status: { code: 503, body: { title: "Service Unavailable", status: 503, detail: "the catalogue cannot be reached" } },
    });
    await page.goto("/projects/helsinki/ckan?lang=en");
    await expect(page.getByRole("alert")).toContainText("the catalogue cannot be reached");
    await expect(page.getByText("No catalogue is configured for this project yet.")).toHaveCount(0);
    await expect(page.getByText("No endpoint in this project publishes to a catalogue.")).toHaveCount(0);
  });

  // PF-107 (T-3046): the administrator hands one catalogue to a group from its Access panel, by
  // keyboard alone; the instance's own role and binding go out as one checked import.
  test("an administrator gives the catalogue to a group in one step, by keyboard", async ({ page }) => {
    const { writes } = await stubApi(page, { admin: true });
    await page.goto("/projects/helsinki/ckan?lang=en");
    const access = page.getByRole("region", { name: "Access to hel-fi" });
    await expect(access.getByRole("button", { name: "Give the right" })).not.toHaveAttribute("aria-disabled", "true");
    await access.getByLabel("Group", { exact: true }).focus();
    await page.keyboard.press("ArrowDown");
    await expect(access.getByLabel("Group", { exact: true })).toHaveValue("ckan-editors");
    await page.keyboard.press("Tab");
    await expect(access.getByRole("button", { name: "Give the right" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(access.getByText("chg-ckan-1")).toBeVisible();
    expect(writes.map((write) => write.path)).toEqual([
      "/api/v1/projects/helsinki/import?dryRun=All",
      "/api/v1/projects/helsinki/import",
    ]);
    expect(writes[1].body).toMatchObject({
      manifests: [
        { kind: "Role", metadata: { name: "ckan-admin-helsinki-hel-fi", namespace: "org" } },
        {
          kind: "RoleBinding",
          metadata: { name: "ckan-admin-helsinki-hel-fi", namespace: "org" },
          spec: { subjects: [{ group: "ckan-editors" }], role: "ckan-admin-helsinki-hel-fi", scope: { project: "helsinki" } },
        },
      ],
    });
  });

  test("a steward sees who manages the catalogue, and the step disabled with the reason", async ({ page }) => {
    const { writes } = await stubApi(page);
    await page.goto("/projects/helsinki/ckan?lang=en");
    const access = page.getByRole("region", { name: "Access to hel-fi" });
    const give = access.getByRole("button", { name: "Give the right" });
    await expect(give).toHaveAttribute("aria-disabled", "true");
    await expect(give).toHaveAccessibleDescription(/Only an administrator who may approve role bindings/);
    await give.click({ force: true });
    expect(writes).toEqual([]);
  });

  test("has no axe violations", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/ckan?lang=en");
    await expect(page.getByRole("table", { name: "Catalogues" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
