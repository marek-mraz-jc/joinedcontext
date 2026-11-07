// T-3589: the model diagram of the seeded Helsinki model (the largest on dev) at the four widths:
// axe-clean, no sideways page scroll, the drawing scrolls inside its own frame. With
// DIAGRAM_SHOTS=<dir> it also saves the 1440 and 375 screenshots the task records.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

const LINKML = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../tests/fixtures/models/helsinki.linkml.yaml"), "utf8");
const IDENTITY = { subject: "s", username: "jana", name: "Jana", email: "jana@hel.fi", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "helsinki", namespace: "helsinki", title: "Helsinki city context" },
  spec: { contextSpaceRef: "helsinki", version: "1.0.0", lifecycle: "published", linkml: LINKML },
};

for (const width of [375, 768, 1024, 1440]) {
  test(`the Helsinki model's diagram at ${width} px is axe-clean and keeps the page from scrolling sideways`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.route("**/api/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: true, grants: [] });
      if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
      if (path.endsWith("/datamodels")) return json(list([MODEL]));
      return json(list([]));
    });
    await page.goto("/projects/helsinki/models/helsinki?lang=en");
    const drawing = page.getByRole("group", { name: "The model's classes and what joins them" });
    await expect(drawing).toBeVisible();
    await expect(drawing.getByRole("button", { name: "Open AirQualityObserved in the structure view" })).toBeVisible();

    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    // T-4991: on load the drawing fits its frame's width and its frame stays under 45rem tall (max-h-180).
    const fit = await drawing.evaluate((svg) => {
      const frame = svg.parentElement as HTMLElement;
      return { over: svg.getBoundingClientRect().width - frame.clientWidth, tall: frame.getBoundingClientRect().height };
    });
    // A phone may hit the 25 % floor and scroll the drawing inside its frame instead.
    if (width >= 1024) expect(fit.over).toBeLessThanOrEqual(0);
    expect(fit.tall).toBeLessThanOrEqual(720 + 2);
    expect(await axeViolations(page)).toEqual([]);
    const shots = process.env.DIAGRAM_SHOTS;
    if (shots && (width === 1440 || width === 375)) {
      await page.screenshot({ path: join(shots, `diagram-${width}.png`), fullPage: true });
    }
  });
}
