/**
 * The Metadata tab tells a viewer nothing their role may not read (T-5529, DM-114).
 *
 * A viewer holds no `readVersions` in the helsinki space: the tab's one `versions=all&lastN=1`
 * read must answer 403 and the tab must say "history not granted" for the writer's fields, while
 * Created and Last changed still come from `sysAttrs`. No answer the viewer's browser receives
 * carries a version's provenance, so the writer's name, organization and reason stay on the server.
 * Nothing is written, so nothing is cleaned up.
 */
import { expect, test } from "@playwright/test";
import { VIEWER, signIn } from "./portal";

const SPACE = "/cs/helsinki/ngsi-ld/v1";

test("a viewer's Metadata tab hides the writer and no answer carries provenance", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, "/projects/helsinki/apps?lang=en");
  try {
    const listed = await page.request.get(`${SPACE}/entities?type=AirQualityObserved&limit=1&options=sysAttrs`);
    expect(listed.status(), "the viewer reads the space").toBe(200);
    const [entity] = (await listed.json()) as Record<string, unknown>[];
    expect(entity, "the helsinki space holds an AirQualityObserved").toBeDefined();
    const id = String(entity.id);
    const attr = Object.keys(entity).find((key) => !["id", "type", "@context", "createdAt", "modifiedAt", "location"].includes(key)) ?? "";

    const versions = await page.request.get(`${SPACE}/temporal/entities/${encodeURIComponent(id)}?attrs=${attr}&versions=all&lastN=1`);
    expect(versions.status(), "a viewer has no readVersions").toBe(403);

    const bodies: string[] = [];
    page.on("response", async (response) => {
      if (response.url().includes("/ngsi-ld/v1/")) bodies.push(await response.text().catch(() => ""));
    });
    await page.goto(`/projects/helsinki/spaces/helsinki?lang=en&row=${encodeURIComponent(id)}`);
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("tab", { name: "Metadata" }).click();
    await dialog.getByRole("combobox", { name: "Attribute" }).selectOption(attr);
    await expect(dialog.getByText("hidden: no access (history not granted)").first()).toBeVisible();
    expect(bodies.length, "the page read the space").toBeGreaterThan(0);
    expect(bodies.filter((body) => body.includes('"provenance"')), "no answer carries a version's provenance").toEqual([]);
  } finally {
    await context.close();
  }
});
