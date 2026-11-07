import { defineConfig, devices } from "@playwright/test";

// The built bundle in a real browser, answered by the SDK's stub from `e2e/serve.ts`: no
// server and no endpoint. Run `pnpm build` first; `e2e/serve.ts` serves `dist/` itself. The build
// lane runs it on its own Chromium after every build (T-2827).
export default defineConfig({
  testDir: "./e2e",
  forbidOnly: Boolean(process.env.CI),
  reporter: process.env.CI ? "line" : "list",
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
