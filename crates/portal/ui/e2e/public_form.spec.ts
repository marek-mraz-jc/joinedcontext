import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// A form view published as a public link (T-3103, T-3172, API/01 §33), opened by somebody who is
// not signed in: the page builds its fields from the Endpoint's published schema, says that what
// is sent is public data, and posts anonymously; the id it shows is the one the gateway minted.

test.describe("a published form", () => {
  test("a visitor fills the form, sees the minted id, and a link that is gone says so", async ({ page }) => {
    const posted: { body: Record<string, unknown>; credentials: boolean }[] = [];
    await page.route("**/api/v1/**", async (route) => {
      const unauthorized = new URL(route.request().url()).pathname.endsWith("/auth/me");
      await route.fulfill({
        status: unauthorized ? 401 : 404,
        contentType: "application/problem+json",
        body: JSON.stringify({ title: unauthorized ? "Unauthorized" : "Not Found", status: unauthorized ? 401 : 404 }),
      });
    });
    await page.route("**/api/endpoint/**", async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const json = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
      if (url.pathname.startsWith("/api/endpoint/gone/")) return json({ title: "Not Found", status: 404 }, 404);
      if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 1 }] });
      if (url.pathname.endsWith("/schema/v1/json-schema")) {
        return json({
          definitions: {
            Report: {
              required: ["id", "type", "description"],
              properties: {
                id: { type: "string" },
                type: { type: "string" },
                description: { type: ["string", "null"], description: "What is wrong, and where" },
              },
            },
          },
        });
      }
      if (request.method() === "POST" && url.pathname.endsWith("/ngsi-ld/v1/entities")) {
        posted.push({ body: request.postDataJSON() as Record<string, unknown>, credentials: "cookie" in (await request.allHeaders()) });
        return route.fulfill({ status: 201, headers: { Location: "/ngsi-ld/v1/entities/urn:ngsi-ld:Report:minted-1" } });
      }
      return json({ title: "Not Found", status: 404 }, 404);
    });

    await page.goto("/f/k7m2qz4tv6xh3n5jb2ryd3wcfa?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Report" })).toBeVisible();
    await expect(page.getByText("Your answers become public data of this space")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.getByRole("textbox", { name: /description/ }).fill("A pothole on Hlavná");
    await page.getByRole("button", { name: "Create" }).click();
    await expect(page.getByText("Created urn:ngsi-ld:Report:minted-1.")).toBeVisible();
    expect(posted).toHaveLength(1);
    expect(posted[0].credentials).toBe(false);
    expect(posted[0].body).not.toHaveProperty("website");

    await page.goto("/f/gone?lang=en");
    await expect(page.getByText("This form is not published, or no longer.")).toBeVisible();
  });
});
