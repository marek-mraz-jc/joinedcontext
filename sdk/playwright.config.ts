import { defineConfig, devices } from "@playwright/test";

// The browser check of the code preview (T-0678): the Portal's own test writes the template's
// document to JC_PREVIEW_OUT, and the spec frames it the way the Portal does. No server: the
// spec answers both addresses itself.
export default defineConfig({
  testDir: "./e2e",
  forbidOnly: Boolean(process.env.CI),
  reporter: process.env.CI ? "github" : "list",
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
