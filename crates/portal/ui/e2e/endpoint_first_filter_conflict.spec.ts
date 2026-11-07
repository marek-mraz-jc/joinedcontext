import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// F-UI2-013: an Endpoint without a projection proposes its first filter as one import that
// creates the projection. A projection of that name created since the page loaded (a concurrent
// proposal, another steward) is refused with the import's detail in the form, never replaced.

const API = "joinedcontext.com/v1alpha1";
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "air", namespace: "helsinki", labels: { "joinedcontext.com/space": "ilma" } },
  spec: { contextSpaceRef: "ilma", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project-list", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
};
const MODEL = {
  apiVersion: API,
  kind: "DataModel",
  metadata: { name: "air-quality", namespace: "helsinki" },
  spec: {
    contextSpaceRef: "ilma",
    version: "1.0.0",
    linkml: [
      "id: https://example.org/air",
      "name: air",
      "classes:",
      "  AirQualityObserved:",
      "    slots: [pm10, location]",
      "slots:",
      "  pm10:",
      "    range: float",
      "  location:",
      "    range: string",
      "",
    ].join("\n"),
  },
};
const SPACE = { apiVersion: API, kind: "ContextSpace", metadata: { name: "ilma", namespace: "helsinki" }, spec: {} };
const TAKEN = "ModelProjection 'air' already exists in project 'helsinki'; choose skip, replace or rename (MF-23)";

for (const width of [1440, 400]) {
  test(`a first filter whose projection name was taken meanwhile is refused in the form at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const imports: unknown[] = [];
    const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
    await page.route("**/api/v1/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) {
        return json({ subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
      }
      if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
      if (path === "/api/v1/projects/helsinki/endpoints/air") return json(ENDPOINT);
      if (path === "/api/v1/projects/helsinki/import") {
        imports.push(route.request().postDataJSON());
        return json({ type: "about:blank", title: "Conflict", status: 409, detail: TAKEN }, 409);
      }
      if (path.endsWith("/datamodels")) return json(list([MODEL]));
      if (path.endsWith("/spaces")) return json(list([SPACE]));
      return json(list([]));
    });
    await page.route("**/cs/**", (route) => route.fulfill({ contentType: "application/ld+json", body: "[]" }));
    await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));

    await page.goto("/projects/helsinki/endpoints/air?lang=en");
    const form = page.getByRole("form", { name: "Filter of this endpoint" });
    await form.locator("#filter-q").fill("pm10>40");
    await form.getByRole("button", { name: "Propose this filter" }).click();

    await expect(form.getByRole("alert")).toContainText("'air' already exists in project 'helsinki'");
    await expect(form.getByRole("alert")).toContainText("Reload the page to propose the filter under a name nobody holds yet.");
    expect(imports).toHaveLength(1);
    expect(imports[0]).toMatchObject({ conflictPolicy: "fail" });

    const dir = join("test-results", "F-UI2-013");
    mkdirSync(dir, { recursive: true });
    await form.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `first-filter-refused-${width}.png`) });
  });
}
