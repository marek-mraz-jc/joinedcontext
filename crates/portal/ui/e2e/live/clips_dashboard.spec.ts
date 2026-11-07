/**
 * T-6454, journey 5: an AI dashboard from the app builder, one prompt to a drawn map. The builder run
 * is paid, so the clip runs only with JC_LIVE_MODEL_RUN=1, once at 1440; the phone clip opens the same
 * build. The run is cancelled in `finally`, so the organization's builder slots stay free, and the
 * `t6454-` draft goes with the residue journey. The builder needs propose on App, which demo.editor
 * lacks, so the clip signs in as the steward: PORTAL_PASSWORD and PORTAL_TOTP.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf } from "./portal";
import { SIZES, clipOf } from "./clips";

const PROJECT = "helsinki";
const ENDPOINT = "helsinki-bikes";
const PROMPT = "Map the shared bike stations, colour-coded by occupancy, and count the empty ones";
const MAP = "canvas.maplibregl-canvas";
const NAME = `t6454-clip-${Date.now().toString(36)}`;

test.setTimeout(900_000);
test.skip(process.env.JC_LIVE_MODEL_RUN !== "1", "a live model run is paid: JC_LIVE_MODEL_RUN=1 with the owner's go");
test.skip(!STEWARD.totp, "the steward signs in with TOTP: PORTAL_TOTP");
test.describe.configure({ mode: "serial" });

test("AI dashboard from the app builder at 1440", async ({ browser }) => {
  const clip = await clipOf(browser, STEWARD, "dashboard", SIZES[0], `/projects/${PROJECT}/apps?lang=en`);
  const { page } = clip;
  let run = "";
  try {
    await clip.step("Applications: the way to ask for an app", async () => {
      await expect(page.getByRole("button", { name: "Generate your own app" }).first()).toBeVisible({ timeout: 60_000 });
    });
    await clip.step("Say what the app should do, over which Endpoint", async () => {
      await page.getByRole("button", { name: "Generate your own app" }).first().click();
      await page.getByLabel("Endpoint").first().selectOption(ENDPOINT);
      await page.getByLabel("What should the app do?").first().fill(PROMPT);
      await page.getByText("Details:", { exact: false }).first().click();
      await page.getByLabel("App name").fill(NAME);
    });
    await clip.step("Generate: the page follows the build step by step", async () => {
      const started = page.waitForResponse((answer) => answer.request().method() === "POST" && answer.url().endsWith(`/projects/${PROJECT}/agent-runs`));
      await page.getByRole("button", { name: "Generate the app" }).click();
      const answer = await started;
      expect(answer.status(), await answer.text()).toBeLessThan(300);
      run = ((await answer.json()) as { id: string }).id;
      await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${NAME}`), { timeout: 60_000 });
    });
    const preview = page.frameLocator("iframe").first();
    await clip.step("The first map stands in the preview while the build goes on", async () => {
      await expect(preview.locator(MAP)).toBeVisible({ timeout: 300_000 });
      await expect(preview.locator(".jc-loading, [aria-busy=true]")).toHaveCount(0, { timeout: 120_000 });
    }, 8_000);
    await clip.step("The build waits for the person: read it, change it or propose it", async () => {
      await expect(page.getByTestId("run-progress").filter({ hasText: "Waiting for you" })).toBeVisible({ timeout: 300_000 });
    }, 8_000);
  } finally {
    await clip.finish().catch((failed: unknown) => console.log(String(failed)));
  }
  const phone = await clipOf(browser, STEWARD, "dashboard", SIZES[1], `/projects/${PROJECT}/apps/${NAME}?lang=en`);
  try {
    await phone.step("The same build on a phone", async () => {
      await expect(phone.page.frameLocator("iframe").first().locator(MAP)).toBeVisible({ timeout: 120_000 });
    }, 8_000);
    await phone.page.mouse.wheel(0, 700);
    await phone.step("Below the map: the progress and what to do next", async () => {
      await expect(phone.page.getByTestId("run-progress")).toBeVisible({ timeout: 30_000 });
    });
  } finally {
    if (run) await phone.page.request.post(`/api/v1/projects/${PROJECT}/agent-runs/${run}/cancel`, { headers: { "x-csrf-token": await csrf(phone.context) } });
    await phone.finish();
  }
});
