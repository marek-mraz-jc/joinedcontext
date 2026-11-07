import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// A run that failed (T-6442): the page says what happened in a person's words and what to do
// next, with the way back to the Assistant; Publish cannot be pressed and Stop is gone. `vite preview` has no API, so it is answered
// here.

const PROJECT = "helsinki";
const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "bike-stations",
  endpointName: "helsinki-bikes",
  appClass: "static",
  visibility: "project",
  prompt: "Map the shared bike stations, colour-coded by occupancy, and count the empty ones",
  status: "failed",
  error: "The Portal's database did not answer in time. Start the run again in a minute.",
  steps: 3,
  tokensUsed: 34_785,
  createdBy: "demo.steward",
  createdAt: "2026-11-02T13:31:40Z",
};

for (const width of [1440, 400]) {
  test(`a failed run explains itself at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.route(/\/api\//, async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.steward", name: "Demo Steward", roles: [] });
      if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
      if (path.endsWith("/agent-runs")) return json({ items: [RUN] });
      if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(RUN);
      if (path.endsWith("/events")) return route.fulfill({ status: 204 });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });
    await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`);

    // On a phone the assistant stands over the page until it is hidden.
    if (width === 400) await page.getByRole("button", { name: "Hide the assistant" }).click();
    await expect(page.getByText(/did not answer in time/)).toContainText("Open the Assistant page to build it again.");
    await expect(page.getByRole("button", { name: "Publish this app" })).toBeDisabled();
    await expect(page.getByText("This run has ended, so it has nothing to publish.")).toBeVisible();
    await expect(page.getByRole("link", { name: "Build it again" })).toHaveAttribute("href", `/projects/${PROJECT}/assistant`);
    await expect(page.getByRole("button", { name: "Stop the run" })).toHaveCount(0);
    expect(await page.locator("body").innerText()).not.toContain("pool timed out");
    expect(await axeViolations(page)).toEqual([]);
    if (process.env.JC_EVIDENCE_DIR) {
      mkdirSync(process.env.JC_EVIDENCE_DIR, { recursive: true });
      await page.screenshot({ path: `${process.env.JC_EVIDENCE_DIR}/run-failed-${width}.png`, fullPage: true });
    }
  });
}
