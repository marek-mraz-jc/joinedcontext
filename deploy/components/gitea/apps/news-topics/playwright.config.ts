import { defineConfig, devices } from "@playwright/test";

// The built bundle in a real browser, answered by the SDK's stub: no server and no endpoint.
// Run `pnpm build` first; the spec serves `dist/` itself.
export default defineConfig({
  testDir: "./e2e",
  forbidOnly: Boolean(process.env.CI),
  reporter: process.env.CI ? "github" : "list",
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
